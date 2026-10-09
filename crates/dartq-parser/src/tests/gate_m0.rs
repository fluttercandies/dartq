//! M0-gate panic regressions (#322): the five corpus files that crashed the
//! first dartiq-vs-oracle smoke (docs/audits/gate-m0/smoke-141/report.json)
//! must flow through `parse_unit` without panicking.
//!
//! Three root-cause families, pinned verbatim (the files are official SDK
//! test corpus members, BSD-licensed, copied unmodified from tests/corpus/):
//!
//! * `gate_m0_f_bounded.dart` / `gate_m0_function_bounded_sealed_classes.dart`
//!   — exhaustiveness data with bounded type parameters: the completion pass
//!   re-parsed each bound at the moved-on cursor instead of its own `extends`
//!   token (official anchor: `typeInfo.ensureTypeNotVoid(next2, parser)`,
//!   `type_info_impl.dart:1571`), and the group element scan underscanned
//!   `id Function(...)` arguments (the official scan is compute-driven,
//!   `type_info_impl.dart:1363-1388`).
//! * `gate_m0_issue_26810{,_and,_or}.dart` — the written-out-operator
//!   recovery family (`and`/`or`/`xor` at binary positions), which used to
//!   panic inside the candidate-level lookup.
//!
//! Contract per file: no panic, the whole stream is consumed, every
//! diagnostic stays inside the source's UTF-16 bounds, and the diagnostic
//! multiset matches the official suite's expectation for the file
//! (`front_end/parser_testcases/error_recovery/issue_26810*.dart.expect`):
//! the exhaustiveness-data files are valid Dart and parse clean; the
//! `issue_26810` family reports exactly 14 written-out-operator recoveries
//! plus the official declaration-misparse of the final `x and y;` statement
//! (the SDK test names that method `not_currently_working`: the official
//! parser rewrites it to `x and ;y;` and reports the missing `;` — analyzer
//! code `expected_token`, official anchor `ExpectedAfterButGot`).

use super::collector::setup;

const F_BOUNDED: &str = include_str!("fixtures/gate_m0_f_bounded.dart");
const FUNCTION_BOUNDED: &str =
    include_str!("fixtures/gate_m0_function_bounded_sealed_classes.dart");
const ISSUE_26810: &str = include_str!("fixtures/gate_m0_issue_26810.dart");
const ISSUE_26810_AND: &str = include_str!("fixtures/gate_m0_issue_26810_and.dart");
const ISSUE_26810_OR: &str = include_str!("fixtures/gate_m0_issue_26810_or.dart");

/// Parses a whole fixture with `parse_unit` and checks the survival
/// contract: EOF reached, diagnostics inside the source bounds.
#[allow(clippy::cast_possible_truncation)] // fixture sources are far below 4G utf16 units
fn parse_unit_survives(source: &'static str, label: &str) -> Vec<String> {
    let utf16_len = source.chars().map(char::len_utf16).sum::<usize>() as u32;
    let mut p = setup(source);
    p.parse_unit();
    assert!(
        p.cursor.at_eof(),
        "{label}: parse_unit must consume the whole stream"
    );
    let (codes, in_bounds) = p.listener.errors.iter().fold(
        (Vec::new(), true),
        |(mut codes, mut in_bounds), (code, start, end)| {
            codes.push(format!("{code}({start}..{end})"));
            in_bounds &= *start <= utf16_len && *end <= utf16_len && start <= end;
            (codes, in_bounds)
        },
    );
    assert!(
        in_bounds,
        "{label}: diagnostics must stay inside the source bounds: {:?}",
        p.listener.errors
    );
    codes
}

/// The official diagnostic multiset for one `issue_26810` file: 14
/// written-out-operator recoveries plus the declaration-misparse of the
/// `not_currently_working` body (counted by code; the official suite fixes
/// the same multiset in `issue_26810*.dart.expect`).
fn assert_issue_26810_recovery(codes: &[String], label: &str) {
    let written_out = codes
        .iter()
        .filter(|c| c.starts_with("binary_operator_written_out"))
        .count();
    let expected_token = codes
        .iter()
        .filter(|c| c.starts_with("expected_token"))
        .count();
    assert_eq!(
        (written_out, expected_token, codes.len()),
        (14, 1, 15),
        "{label}: official recovery shape is 14 written-out operators + \
         the `x and y;` declaration misparse: {codes:?}"
    );
}

#[test]
fn f_bounded_type_parameters_parse_without_panic() {
    let codes = parse_unit_survives(F_BOUNDED, "f_bounded");
    assert!(
        codes.is_empty(),
        "valid exhaustiveness data must parse clean: {codes:?}"
    );
}

#[test]
fn function_bounded_sealed_classes_parse_without_panic() {
    let codes = parse_unit_survives(FUNCTION_BOUNDED, "function_bounded");
    assert!(
        codes.is_empty(),
        "valid exhaustiveness data must parse clean: {codes:?}"
    );
}

#[test]
fn issue_26810_xor_recovery_without_panic() {
    assert_issue_26810_recovery(
        &parse_unit_survives(ISSUE_26810, "issue_26810"),
        "issue_26810",
    );
}

#[test]
fn issue_26810_and_recovery_without_panic() {
    assert_issue_26810_recovery(
        &parse_unit_survives(ISSUE_26810_AND, "issue_26810_and"),
        "issue_26810_and",
    );
}

#[test]
fn issue_26810_or_recovery_without_panic() {
    assert_issue_26810_recovery(
        &parse_unit_survives(ISSUE_26810_OR, "issue_26810_or"),
        "issue_26810_or",
    );
}

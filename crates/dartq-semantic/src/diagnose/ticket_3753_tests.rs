//! The #3753 pins — dot shorthands used as the LEFT operand of `is`/`as`.
//!
//! Upstream's `binary_expression_resolver.dart` analyzes a type test's and
//! a cast's operands under the unknown schema: neither operator pushes a
//! dot-shorthand context, so a shorthand rooted at that operand resolves
//! against NO context at all. The read arm reports
//! `dot_shorthand_missing_context` over the whole shorthand span
//! (`property_element_resolver.dart:129`), never a
//! `dot_shorthand_undefined_member` against the enclosing bool position the
//! operand happens to sit in (`if (.one is C)` reports `missing_context`,
//! not a `'bool'` member miss).
//!
//! Oracle provenance: `dart analyze` 3.13.7 on
//! `co19/LanguageFeatures/Static-access-shorthand/grammar_A06_t04.dart`
//! (the `if (.one is C)` family) and on a reduced `(C c) => c == .one`
//! probe for the retained equality face.

use super::test_rig::{CORE_PRELUDE, report_ops};
use crate::diagnose::FileReport;

/// Fixture-local byte offset of the first `needle`, shifted by the prelude
/// [`report_ops`] prepends.
fn at(fixture: &str, needle: &str) -> u32 {
    let hit = fixture
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in fixture:\n{fixture}"));
    u32::try_from(CORE_PRELUDE.len() + hit).expect("fixture offsets fit u32")
}

/// The rows of `code` as `(offset, length)` pairs, ascending.
fn rows_of(report: &FileReport, code: &str) -> Vec<(u32, u32)> {
    let mut rows: Vec<(u32, u32)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.range.end() - d.range.offset))
        .collect();
    rows.sort_unstable();
    rows
}

const SHORTHAND_CLASS: &str = "\
class C {
  static C one = C();
  static C two = C();
  C();
}
";

/// `if (.one is C)` — the type-test operand arms no context: one
/// whole-span `missing_context`, zero member misses.
#[test]
fn is_operand_shorthand_reports_missing_context() {
    let fixture = format!(
        "{SHORTHAND_CLASS}\nvoid main() {{\n  if (.one is C) {{}}\n  if (.two is C) {{}}\n}}\n"
    );
    let report = report_ops(&fixture);
    assert_eq!(
        rows_of(&report, "dot_shorthand_missing_context"),
        vec![(at(&fixture, ".one is"), 4), (at(&fixture, ".two is"), 4)],
        "the `is` LHS shorthand reports its whole span"
    );
    assert!(
        rows_of(&report, "dot_shorthand_undefined_member").is_empty(),
        "no `bool` member miss leaks from the enclosing condition"
    );
}

/// `.one as C` — the cast operand arms no context either.
#[test]
fn as_operand_shorthand_reports_missing_context() {
    let fixture =
        format!("{SHORTHAND_CLASS}\nvoid main() {{\n  if ((.one as C) == .two) {{}}\n}}\n");
    let report = report_ops(&fixture);
    assert_eq!(
        rows_of(&report, "dot_shorthand_missing_context"),
        vec![(at(&fixture, ".one as"), 4)],
        "the cast LHS shorthand reports its whole span"
    );
    // The cast RESULT feeds the `==` left operand: `.two` reads `C` and
    // resolves, so the equality face stays silent.
    assert!(
        rows_of(&report, "dot_shorthand_undefined_member").is_empty(),
        "the resolved equality operand stays silent"
    );
}

/// The retained equality face: a DIRECT shorthand right of `==` still
/// reads the left operand's type.
#[test]
fn equality_right_shorthand_still_reads_left_type() {
    let fixture = format!(
        "{SHORTHAND_CLASS}\nvoid main(C c) {{\n  if (c == .one) {{}}\n  if (c == .bogus) {{}}\n}}\n"
    );
    let report = report_ops(&fixture);
    assert_eq!(
        rows_of(&report, "dot_shorthand_undefined_member"),
        vec![(at(&fixture, "bogus"), 5)],
        "the miss resolves against `C`, the hit stays silent"
    );
    assert!(
        rows_of(&report, "dot_shorthand_missing_context").is_empty(),
        "the equality operand arms a real context"
    );
}

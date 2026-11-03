//! #3641 pins — S1-B refutable field-columns: the deferred-row machinery
//! extended to record field judging (the arm this ticket can land; the
//! list/map/wildcard/unnecessary-* arms sit in `engine_core.rs` +
//! `listener.rs`, held by live lanes — their verdicts pin current behavior
//! where the oracle face already passes, and the held diffs ride the
//! ticket report).
//!
//! Upstream dispatches every record field's sub-pattern with that field's
//! own slot type as `matchedValueType` (`analyzeRecordPattern`, upstream
//! Dart SDK source, pkg/_fe_analyzer_shared/lib/src/type_inference/
//! type_analyzer.dart:1841-1855), so `checkPatternNeverMatchesValueType`
//! (pkg/analyzer/lib/src/generated/resolver.dart:624-653) judges a field
//! against its column, never the whole record.
//!
//! Registered through `diagnose/tests.rs`'s `#[path]` block (`diagnose.rs`
//! is a live-lane file out of this ticket's write scope).

use crate::diagnose::test_rig::{CORE_PRELUDE, report_ops};
use crate::diagnose::types::FileReport;

/// The `(offset, length, message)` of one code's rows, ascending by offset.
fn rows(report: &FileReport, code: &str) -> Vec<(u32, u32, String)> {
    let mut rows = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect::<Vec<_>>();
    rows.sort_by_key(|row| row.0);
    rows
}

/// The absolute offset of the `nth` occurrence (0-based) of `needle` in the
/// fixture as the rig analyzes it: `report_ops` prefixes [`CORE_PRELUDE`],
/// so every fixture-local offset shifts by the prelude's length.
fn at(fixture: &str, needle: &str, nth: usize) -> u32 {
    let mut from = 0;
    for _ in 0..nth {
        from += fixture[from..]
            .find(needle)
            .expect("needle appears in the fixture")
            + needle.len();
    }
    let hit = from
        + fixture[from..]
            .find(needle)
            .expect("needle appears in the fixture");
    u32::try_from(CORE_PRELUDE.len() + hit).expect("fixture offsets fit u32")
}

/// `needle`'s length as the expected row width.
fn at_len(needle: &str) -> u32 {
    u32::try_from(needle.len()).expect("fixture needles fit u32")
}

/// The never-match message for a column-vs-required verdict.
fn never_matches(matched: &str, required: &str) -> String {
    format!(
        "The matched value type '{matched}' can never match the required type '{required}'."
    )
}

// —— record field-columns (the in-scope arm) ———————————————————————————

/// A positional record field's sub-pattern is judged against the field's
/// own slot type (`matchedType.positional[i]`), not the whole record: the
/// `F` slot of `(E, String)` can never hold an `F`-required pattern.
#[test]
fn record_positional_field_is_judged_against_its_slot() {
    let fixture = "\
enum E { a }
enum F { b }
void f((E, String) r) {
  switch (r) {
    case (E a, String s): a; s; break;
    case (F x, String s): x; s; break;
  }
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "pattern_never_matches_value_type"),
        vec![(
            at(fixture, "F x", 0),
            1,
            never_matches("E", "F"),
        )],
        "the first slot's column is `E`: {:?}",
        report.diagnostics
    );
}

/// A named record field reads the named slot's type
/// (`matchedType.named[n]`): `(E, {String s})`'s `s:` column is `String`,
/// which an `F`-required sub-pattern can never satisfy.
#[test]
fn record_named_field_is_judged_against_its_slot() {
    let fixture = "\
enum F { b }
void f((int, {String s}) r) {
  switch (r) {
    case (i, s: F x): i; x; break;
  }
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "pattern_never_matches_value_type"),
        vec![(
            at(fixture, "F x", 0),
            1,
            never_matches("String", "F"),
        )],
        "the named slot `s` is `String`: {:?}",
        report.diagnostics
    );
}

/// The negative control: a column that CAN hold the required type stays
/// silent — `int` admits `num` patterns (the slot type is assignable to
/// the required type), so no row fires.
#[test]
fn record_field_column_admitting_the_type_stays_silent() {
    let fixture = "\
void f((int, String) r) {
  switch (r) {
    case (num a, String s): a; s; break;
  }
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "pattern_never_matches_value_type"),
        vec![],
        "`int` admits `num`: {:?}",
        report.diagnostics
    );
}

/// The irrefutable-context gate: a declaration-context record
/// destructure reports no `pattern_never_matches_value_type` at all — the
/// official's `irrefutableContext == null` arm owns the row family
/// (`pattern_type_mismatch_in_irrefutable_context` is its own face, wave
/// A's scope).
#[test]
fn record_field_stays_silent_in_declaration_context() {
    let fixture = "\
enum E { a }
enum F { b }
void f((E, String) r) {
  var (F x, String s) = r;
  x; s;
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "pattern_never_matches_value_type"),
        vec![],
        "declaration records judge no field columns: {:?}",
        report.diagnostics
    );
}

/// A second positional field reads the SECOND positional slot — the
/// index counts positional fields alone, not named ones between them.
#[test]
fn record_positional_index_counts_positional_slots() {
    let fixture = "\
enum F { b }
void f((int, {String s}) r) {
  switch (r) {
    case (i, s: var v): i; v; break;
  }
}
";
    let report = report_ops(fixture);
    // Only one positional field; the named slot is `String`.
    assert_eq!(
        rows(&report, "pattern_never_matches_value_type"),
        vec![],
        "`int` admits `var v` and `String` admits `var v`: {:?}",
        report.diagnostics
    );
}

/// A nested record field is still silent: `pattern_matched_ty` reads the
/// scrutinee only under all-binary frames, so a record inside a record
/// keeps the established under-report — never a guessed column.
#[test]
fn nested_record_field_keeps_the_under_report() {
    let fixture = "\
enum F { b }
void f(((int,), String) r) {
  switch (r) {
    case ((F x,), String s): x; s; break;
  }
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "pattern_never_matches_value_type"),
        vec![],
        "a nested record's field column is not derivable yet: {:?}",
        report.diagnostics
    );
}

// —— list field-columns (held: listener.rs/engine_core.rs) ————————————————
//
// The list arm needs `handle_pattern_field`-style boundaries the list
// parser never fires, plus `check_list_pattern`'s written-args consult —
// both held files. The pins below lock the current-correct faces so the
// wave's own diffs cannot regress them.

/// The existing list-kind never-match at a root list pattern keeps
/// firing: a matched face that cannot be a `List` at all proves the
/// pattern dead. Regression control for the wave's list-column work.
#[test]
fn root_list_pattern_still_judges_against_the_scrutinee() {
    let fixture = "\
void f(int i) {
  switch (i) {
    case [a, b]: a; b; break;
  }
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "pattern_never_matches_value_type"),
        vec![(
            at(fixture, "[a, b]", 0),
            at_len("[a, b]"),
            never_matches("int", "List<Object?>"),
        )],
        "`int` can never be a `List`: {:?}",
        report.diagnostics
    );
}

/// The map arm: the Map kind of `checkPatternNeverMatchesValueType` landed
/// with #3721 (`handle_map_pattern`'s head-row emit) — a `{...}` over `int`
/// reports the oracle's `Map<Object?, Object?>` row at the whole node.
#[test]
fn root_map_pattern_reports_its_head_row() {
    let fixture = "\
void f(int i) {
  switch (i) {
    case {1: var a}: a; break;
  }
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "pattern_never_matches_value_type"),
        vec![(
            at(fixture, "{1: var a}", 0),
            at_len("{1: var a}"),
            never_matches("int", "Map<Object?, Object?>"),
        )],
        "`int` can never be a `Map`: {:?}",
        report.diagnostics
    );
}

// —— wildcard span + gate (held) ————————————————————————————————————————

/// The logical-and operand face keeps reporting the wildcard: both `&&`
/// operands of one `&&` report (`unnecessaryWildcardPattern`,
/// shared_type_analyzer.dart:250-258). Regression control for the span
/// widening the held arm owns.
#[test]
fn wildcard_logical_and_operands_still_report() {
    let fixture = "\
void f(int i) {
  switch (i) {
    case _ && _ : break;
  }
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "unnecessary_wildcard_pattern").len(),
        2,
        "both `&&` operands report: {:?}",
        report.diagnostics
    );
}

/// A typed wildcard whose declaration is already below the matched type
/// reports — `sameMatchedType` (`int _` over `int`).
#[test]
fn wildcard_typed_same_matched_type_reports() {
    let fixture = "\
void f(int i) {
  switch (i) {
    case int _ && > 0: break;
  }
}
";
    let report = report_ops(fixture);
    let wildcard_rows = rows(&report, "unnecessary_wildcard_pattern");
    assert_eq!(
        wildcard_rows.len(),
        1,
        "the typed `int _` reports once: {:?}",
        report.diagnostics
    );
    assert_eq!(wildcard_rows[0].2, "Unnecessary wildcard pattern.");
}

// —— relational pair (held; the codes do not exist yet) ———————————————————

/// The definedness twin keeps reporting `undefined_operator` for a
/// matched class without the spelled operator — the existing
/// `resolveRelationalPatternOperator` consult the pair's emission joins.
#[test]
fn relational_undefined_operator_still_reports() {
    let fixture = "\
class C {}
void f(C c) {
  switch (c) {
    case > 0: break;
  }
}
";
    let report = report_ops(fixture);
    let op_rows = rows(&report, "undefined_operator");
    assert_eq!(
        op_rows.len(),
        1,
        "`C` declares no `>`: {:?}",
        report.diagnostics
    );
    assert_eq!(
        op_rows[0].2,
        "The operator '>' isn't defined for the type 'C'."
    );
}

/// `==`/`!=` never report the operand/operator pair — both spell
/// `operator ==` on `Object` (the `Object`-lookup arm), so the codes stay
/// silent even once the variants exist.
#[test]
fn relational_equality_never_reports_operand_type() {
    let fixture = "\
void f(int i) {
  switch (i) {
    case == 0: break;
  }
}
";
    let report = report_ops(fixture);
    for code in [
        "relational_pattern_operand_type_not_assignable",
        "relational_pattern_operator_return_type_not_assignable_to_bool",
    ] {
        assert_eq!(
            rows(&report, code),
            vec![],
            "`==` resolves on `Object`: {:?}",
            report.diagnostics
        );
    }
}

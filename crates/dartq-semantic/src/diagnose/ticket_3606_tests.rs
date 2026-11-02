//! #3606 — the `unnecessary_type_check` residues the tear-off lane could not
//! reach: the operand side's grounding, not the verdict walk.
//!
//! # The defect this round fixes
//!
//! A collection literal's `for` element opens its loop variable's declaration
//! group and initializer *inside* the enclosing declaration's initializer.
//! Both windows were single slots on the engine (`pending_decl_ty`,
//! `init_mark`), so the nested close wiped the outer one: `List<int> v =
//! <num>[for (int i = 0; i < 3; i++) i]` lost its declared target (no
//! `invalid_assignment`) and `var v = <int>[for (int i = 0; i < 3; i++)
//! i]` lost its inferred type (a later `v is List<int>` stayed silent). The
//! listener parks the enclosing window on its own stacks and hands it back
//! at the nested close.
//!
//! Every expectation below is oracle-probed (`dart analyze --format=machine`,
//! SDK 3.13.7); each pin carries its silent control — a pair the analyzer
//! cannot ground stays silent (the under-report direction, never a
//! fabricated row).

use super::test_rig::{CORE_PRELUDE, report_ops};

/// The fixture-relative byte offset of `needle` (the rig mounts
/// [`CORE_PRELUDE`] ahead of the fixture, so the report's UTF-16 offsets are
/// prelude-shifted; the ASCII fixtures keep byte == UTF-16 offsets).
fn at(fixture: &str, needle: &str) -> u32 {
    let index = fixture
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in fixture"));
    CORE_PRELUDE.len() as u32 + index as u32
}

/// One code's rows as `(offset, length, message)`, in report order.
fn rows_of(report: &super::FileReport, code: &str) -> Vec<(u32, u32, String)> {
    report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == code)
        .map(|row| (row.range.offset, row.range.length, row.message.to_string()))
        .collect()
}

/// A typed list literal's `for` element must not take the inferred local's
/// declared type away: the local folds to the literal's written `List<int>`,
/// so the later `is List<int>` is a proven constant.
///
/// Oracle 3.13.7 (`var v = <int>[for (int i = 0; i < 3; i++) i]; if (v is
/// List<int>) {}`): one `unnecessary_type_check` at `v is List<int>` (len 14).
#[test]
fn for_element_list_literal_grounds_the_inferred_local() {
    let fixture = "void main() {\n  var v = <int>[for (int i = 0; i < 3; i++) i];\n  if (v is List<int>) {}\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        rows_of(&report, "unnecessary_type_check"),
        [(
            at(fixture, "v is List<int>"),
            14,
            "Unnecessary type check; the result is always 'true'.".to_string(),
        )],
        "{:#?}",
        super::test_rig::rows(&report)
    );
}

/// Silent control: the same grounded `List<int>` local against a different
/// element type is a real runtime question.
///
/// Oracle 3.13.7 (`if (v is List<String>) {}`): silent.
#[test]
fn for_element_list_literal_non_matching_check_stays_silent() {
    let fixture = "void main() {\n  var v = <int>[for (int i = 0; i < 3; i++) i];\n  if (v is List<String>) {}\n}\n";
    let report = report_ops(fixture);
    assert!(
        rows_of(&report, "unnecessary_type_check").is_empty(),
        "{:#?}",
        super::test_rig::rows(&report)
    );
}

/// The declared-target half of the same window: the typed literal's
/// `List<num>` reaches the initializer check even when the literal opens with
/// a `for` element.
///
/// Oracle 3.13.7 (`List<int> u = <num>[for (int i = 0; i < 3; i++) i];`):
/// `A value of type 'List<num>' can't be assigned to a variable of type
/// 'List<int>'.` at the literal (len 36).
#[test]
fn for_element_typed_literal_reports_invalid_assignment() {
    let fixture = "void main() {\n  List<int> u = <num>[for (int i = 0; i < 3; i++) i];\n  print(u);\n}\n";
    let report = report_ops(fixture);
    assert_eq!(
        rows_of(&report, "invalid_assignment"),
        [(
            at(fixture, "<num>[for (int i = 0; i < 3; i++) i]"),
            36,
            "A value of type 'List<num>' can't be assigned to a variable of type 'List<int>'."
                .to_string(),
        )],
        "{:#?}",
        super::test_rig::rows(&report)
    );
}

/// Silent control: the declared target matches the literal's own element
/// type.
///
/// Oracle 3.13.7 (`List<num> u = <num>[for (int i = 0; i < 3; i++) i];`):
/// silent.
#[test]
fn for_element_typed_literal_matching_target_stays_silent() {
    let fixture = "void main() {\n  List<num> u = <num>[for (int i = 0; i < 3; i++) i];\n  print(u);\n}\n";
    let report = report_ops(fixture);
    assert!(
        rows_of(&report, "invalid_assignment").is_empty(),
        "{:#?}",
        super::test_rig::rows(&report)
    );
}

/// The set and map heads close their windows the same way: a `for` element
/// under `<int>{}` / `<int, int>{}` still types the inferred local.
///
/// Oracle 3.13.7 (`var s = <int>{for (int i = 0; i < 3; i++) i}; if (s is
/// Set<int>) {}` and the map sibling): `unnecessary_type_check` at
/// `s is Set<int>` (len 13) and `m is Map<int, int>` (len 18).
#[test]
fn for_element_set_and_map_literals_ground_their_locals() {
    let fixture = "void main() {\n  var s = <int>{for (int i = 0; i < 3; i++) i};\n  if (s is Set<int>) {}\n  var m = <int, int>{for (int i = 0; i < 3; i++) i: i};\n  if (m is Map<int, int>) {}\n}\n";
    let report = report_ops(fixture);
    let message = "Unnecessary type check; the result is always 'true'.".to_string();
    assert_eq!(
        rows_of(&report, "unnecessary_type_check"),
        [
            (at(fixture, "s is Set<int>"), 13, message.clone()),
            (at(fixture, "m is Map<int, int>"), 18, message),
        ],
        "{:#?}",
        super::test_rig::rows(&report)
    );
}

/// Silent control for the set face: `Set<int>` is not a `Set<String>`.
///
/// Oracle 3.13.7 (`if (s is Set<String>) {}`): silent.
#[test]
fn for_element_set_literal_non_matching_check_stays_silent() {
    let fixture = "void main() {\n  var s = <int>{for (int i = 0; i < 3; i++) i};\n  if (s is Set<String>) {}\n}\n";
    let report = report_ops(fixture);
    assert!(
        rows_of(&report, "unnecessary_type_check").is_empty(),
        "{:#?}",
        super::test_rig::rows(&report)
    );
}

/// The written-type loop variable (`for (int i = …)`) and the inferred one
/// (`for (var i = …)`) both keep the window; neither shape may regress.
///
/// Oracle 3.13.7: both `if (… is List<int>) {}` checks fire (len 14 each).
#[test]
fn for_element_window_survives_both_loop_variable_spellings() {
    let fixture = "void main() {\n  var a = <int>[for (int i = 0; i < 3; i++) i];\n  if (a is List<int>) {}\n  var b = <int>[for (var i = 0; i < 3; i++) i];\n  if (b is List<int>) {}\n}\n";
    let report = report_ops(fixture);
    let message = "Unnecessary type check; the result is always 'true'.".to_string();
    assert_eq!(
        rows_of(&report, "unnecessary_type_check"),
        [
            (at(fixture, "a is List<int>"), 14, message.clone()),
            (at(fixture, "b is List<int>"), 14, message),
        ],
        "{:#?}",
        super::test_rig::rows(&report)
    );
}

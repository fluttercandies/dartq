//! #3242 pins — #3235's two blocked residuals, both supply-side: the Record
//! kind of `pattern_never_matches_value_type` (it had no call site at all) and
//! the per-node field-column matched type (a container field's sub-pattern is
//! judged against that field's own column, never the scrutinee).
//!
//! Upstream runs `checkPatternNeverMatchesValueType` from every pattern
//! node's `resolvePattern` with that node's own `matchedValueType` (upstream
//! Dart SDK source, pkg/analyzer/lib/src/generated/resolver.dart:1632-1637).
//! A record pattern's required type is the record over its WRITTEN fields
//! with every slot `Object?` (`analyzeRecordPattern`,
//! pkg/_fe_analyzer_shared/lib/src/type_inference/type_analyzer.dart:
//! 1841-1855), and against a record right only an `Object`/`Record`
//! interface left can match — a function left never does
//! (pkg/analyzer/lib/src/dart/element/type_system.dart:269-285). An object
//! pattern's field sub-pattern is resolved against the property's own type
//! (`resolveObjectPatternPropertyGet`, type_analyzer.dart:1674-1697).
//!
//! Registered through `diagnose/tests.rs`'s `#[path]` block (`diagnose.rs` is
//! a live-lane file out of this ticket's write scope).

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

/// The never-match row an `E` scrutinee owes for a record pattern of two
/// positional fields.
fn two_slot_required() -> String {
    "The matched value type 'E' can never match the required type '(Object?, Object?)'."
        .to_string()
}

/// The Record kind runs the never-match verdict at its own node: a required
/// record type can hold no value of an interface the record cannot be seen
/// through (`E` is an enum — `canBeSubtypeOf`'s enum scan never reaches a
/// record). The row spans the whole `(...)` node (the official `.at(pattern)`).
///
/// Before #3242 the kind had no call site at all, so the switch stayed silent.
#[test]
fn record_kind_never_matches_fires() {
    let fixture = "\
enum E { a }
void f(E e) {
  switch (e) {
    case (a, b): a; b; break;
    case E _: break;
  }
}
";
    let report = report_ops(fixture);
    let needle = "(a, b)";
    assert_eq!(
        rows(&report, "pattern_never_matches_value_type"),
        vec![(at(fixture, needle, 0), at_len(needle), two_slot_required())],
        "the record pattern is judged against the scrutinee `E`: {:?}",
        report.diagnostics
    );
}

/// The required type renders the WRITTEN field shape: named fields carry their
/// names in the record type's own display form, every slot `Object?`.
#[test]
fn record_kind_renders_the_named_required_set() {
    let fixture = "\
class C {}
void f(C c) {
  switch (c) {
    case (x: var v): v; break;
  }
}
";
    let report = report_ops(fixture);
    let needle = "(x: var v)";
    assert_eq!(
        rows(&report, "pattern_never_matches_value_type"),
        vec![(
            at(fixture, needle, 0),
            at_len(needle),
            "The matched value type 'C' can never match the required type '({Object? x})'."
                .to_string(),
        )],
        "the named required set renders every written field: {:?}",
        report.diagnostics
    );
}

/// The false-positive guard: a matched type a record value CAN be seen
/// through (`Object?`) leaves the record kind silent — an `Object` value may
/// be a record, so nothing is proven.
#[test]
fn record_kind_stays_silent_over_object() {
    let fixture = "\
void f(Object? o) {
  switch (o) {
    case (a, b): a; b; break;
  }
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "pattern_never_matches_value_type"),
        vec![],
        "an `Object?` matched value may be a record: {:?}",
        report.diagnostics
    );
}

/// Both operands of a logical-or run the Record kind: each keeps the
/// enclosing matched type (`analyzeLogicalOrPattern` dispatches both operands
/// with the matched type, type_analyzer.dart:1112-1200), and each record
/// pattern's `(` latch is its own frame's — a shared latch would drop the
/// left operand's row. The anchor resolves both operands through the
/// scanner's group table, so the left row spans `(a, b)` and not the whole
/// `(a, b) || (c, d)` chain.
#[test]
fn record_kind_reports_each_logical_operand() {
    let fixture = "\
enum E { a }
void f(E e) {
  switch (e) {
    case (a, b) || (c, d): break;
    case E _: break;
  }
}
";
    let report = report_ops(fixture);
    let left = "(a, b)";
    let right = "(c, d)";
    assert_eq!(
        rows(&report, "pattern_never_matches_value_type"),
        vec![
            (at(fixture, left, 0), at_len(left), two_slot_required()),
            (at(fixture, right, 0), at_len(right), two_slot_required()),
        ],
        "both logical operands are judged against the scrutinee `E`: {:?}",
        report.diagnostics
    );
}

/// A record pattern carrying a postfix keeps its own anchor: the frame's end
/// token is the cast's type (`as Object`), so the row must still span `(a, b)`
/// — the group table resolves the `)`.
#[test]
fn record_kind_keeps_its_span_under_a_postfix() {
    let fixture = "\
enum E { a }
void f(E e) {
  switch (e) {
    case (a, b) as Object: break;
    case E _: break;
  }
}
";
    let report = report_ops(fixture);
    let needle = "(a, b)";
    assert_eq!(
        rows(&report, "pattern_never_matches_value_type"),
        vec![(at(fixture, needle, 0), at_len(needle), two_slot_required())],
        "the record anchor stops at its own closer: {:?}",
        report.diagnostics
    );
}

/// A record-typed matched value compares SHAPES: the written record type
/// `(int, String)` cannot inhabit a three-slot required type, so the pattern
/// is dead. The required render is the written field count over `Object?`
/// slots (`canBeSubtypeOf`'s positional walk, type_system.dart:287-330).
#[test]
fn record_kind_compares_record_shapes() {
    let fixture = "\
void f((int, String) r) {
  switch (r) {
    case (a, b, c): a; b; c; break;
  }
}
";
    let report = report_ops(fixture);
    let needle = "(a, b, c)";
    assert_eq!(
        rows(&report, "pattern_never_matches_value_type"),
        vec![(
            at(fixture, needle, 0),
            at_len(needle),
            "The matched value type '(int, String)' can never match the required type \
             '(Object?, Object?, Object?)'."
                .to_string(),
        )],
        "a two-slot record never matches a three-slot pattern: {:?}",
        report.diagnostics
    );
}

/// The per-node field-column face: a field sub-pattern is judged against the
/// property column (`Box.value` is `E`), not the scrutinee — so a required
/// `F` proves the field dead where the scrutinee-shaped judgement would have
/// said nothing (`E` cannot inhabit `F` either, but the row's matched value
/// must be the column).
///
/// Before #3242 the nested node stayed silent (`pattern_matched_ty` answered
/// `None` for every field sub-pattern).
#[test]
fn object_field_sub_pattern_is_judged_against_its_column() {
    let fixture = "\
enum E { a }
enum F { b }
class Box {
  final E value;
  const Box(this.value);
}
void f(E e) {
  if (e case Box(value: F x)) {
    x;
  }
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "pattern_never_matches_value_type"),
        vec![
            // The object head against the scrutinee (`E` never inhabits `Box`);
            // the row spans the head's own token (`pattern.type`).
            (
                at(fixture, "Box(value", 0),
                3,
                "The matched value type 'E' can never match the required type 'Box'."
                    .to_string(),
            ),
            // The field against its column (`Box.value` is `E`, never `F`).
            (
                at(fixture, "F x", 0),
                1,
                "The matched value type 'E' can never match the required type 'F'.".to_string(),
            ),
        ],
        "the field sub-pattern is judged against the `value` column: {:?}",
        report.diagnostics
    );
}

/// The same column face stays silent when the column permits the type: a
/// sealed/enum column admits every value of a type below it, so `Object` (the
/// column) can be an `F`. No row, and no row for the field's column either.
#[test]
fn object_field_column_permitting_the_type_stays_silent() {
    let fixture = "\
enum F { b }
class Box {
  final Object value;
  const Box(this.value);
}
void f(Box b) {
  if (b case Box(value: F x)) {
    x;
  }
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "pattern_never_matches_value_type"),
        vec![],
        "an `Object` column can hold an `F`: {:?}",
        report.diagnostics
    );
}

/// The wildcard kind rides the same column supply: a typed wildcard field
/// (`value: F _`) is judged against the property column, anchored at the
/// written type (`pattern.type`).
#[test]
fn object_field_wildcard_is_judged_against_its_column() {
    let fixture = "\
enum E { a }
enum F { b }
class Box {
  final E value;
  const Box(this.value);
}
void f(Box b) {
  if (b case Box(value: F _)) {
    b;
  }
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "pattern_never_matches_value_type"),
        vec![(
            at(fixture, "F _", 0),
            1,
            "The matched value type 'E' can never match the required type 'F'.".to_string(),
        )],
        "the typed wildcard is judged against the `value` column: {:?}",
        report.diagnostics
    );
}

/// A `u32` field length for a fixture needle.
fn at_len(needle: &str) -> u32 {
    u32::try_from(needle.len()).expect("needle fits u32")
}

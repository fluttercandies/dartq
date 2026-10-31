//! #3235 pins — the never-matches supply gate: the verdict runs at every
//! pattern NODE whose matched type is reachable, not only at the case root.
//!
//! Upstream resolves every pattern node with its own `matchedValueType` and
//! reports `checkPatternNeverMatchesValueType` from that node
//! (`DartPatternImpl.resolvePattern`, upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/ast/ast.dart:4138/8564/24753/27989/35197 and
//! pkg/analyzer/lib/src/dart/resolver/list_pattern_resolver.dart:45); a
//! logical operand keeps its enclosing pattern's matched type
//! (`analyzeLogicalOrPattern`/`analyzeLogicalAndPattern` dispatch both
//! operands with `flow.getMatchedValueType()`, upstream
//! pkg/_fe_analyzer_shared/lib/src/type_inference/type_analyzer.dart:1112-1200).
//!
//! dartq gated the family on `pattern_stack.len() == 1`, so a `||`/`&&`
//! RIGHT operand — which opens its own pattern frame and is still matched
//! against the scrutinee — never ran the verdict. `Engine::pattern_matched_ty`
//! replaces that root-only test with the matched-type predicate (the open
//! scrutinee while every frame above the root is a logical operand), which
//! is exactly the condition under which the scrutinee IS the node's matched
//! type; a container field/element sub-pattern (its matched type is a
//! property column or slot) stays silent — feeding it the scrutinee would
//! judge the wrong value, the false-positive direction.
//!
//! Registered through `diagnose/tests.rs`'s `#[path]` block (`diagnose.rs`
//! is a live-lane file out of this ticket's write scope).

use crate::diagnose::test_rig::{CORE_PRELUDE, report_ops};
use crate::diagnose::types::FileReport;

/// The offsets of one code's rows, ascending.
fn offsets(report: &FileReport, code: &str) -> Vec<u32> {
    let mut at = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| d.range.offset)
        .collect::<Vec<_>>();
    at.sort_unstable();
    at
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

/// A `||` RIGHT operand runs the never-match verdict: its own pattern frame
/// still carries the enclosing scrutinee as the matched type, so a required
/// type no scrutinee value can inhabit proves the operand dead. The closed
/// enum is the name level's decidable disjoint pair (`canBeSubtypeOf`'s
/// enum arm, type_system.dart:229-241).
///
/// Before #3235 the right operand was gated off (`pattern_stack.len() != 1`)
/// and the switch reported nothing; the anchor is the operand's written type
/// (`pattern.type`), not the bound variable.
#[test]
fn logical_or_right_operand_runs_the_never_match_verdict() {
    let fixture = "\
enum E { a }
class A {}
void f(E e) {
  switch (e) {
    case E.a || A x: x; break;
    case E _: break;
  }
}
";
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "pattern_never_matches_value_type"),
        vec![at(fixture, "A x", 0)],
        "the `||` right operand is judged against the scrutinee `E`: {:?}",
        report.diagnostics
    );
}

/// The `&&` RIGHT operand is the same face (a typed wildcard there): the
/// logical-and dispatch hands both operands the enclosing matched type, so
/// the wildcard's declared type is judged against `E`. The wildcard's own
/// `unnecessary_wildcard_pattern` face stays silent — `E` is not below `A`.
#[test]
fn logical_and_right_operand_runs_the_never_match_verdict() {
    let fixture = "\
enum E { a }
class A {}
void f(E e) {
  switch (e) {
    case E.a && A _: break;
    case E _: break;
  }
}
";
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "pattern_never_matches_value_type"),
        vec![at(fixture, "A _", 0)],
        "the `&&` right operand is judged against the scrutinee `E`: {:?}",
        report.diagnostics
    );
}

/// The case root keeps its verdict (the gate replacement must not narrow
/// the face it replaced): the same disjoint pair at a root pattern still
/// reports at the written type's span.
#[test]
fn case_root_never_match_is_unchanged() {
    let fixture = "\
enum E { a }
class A {}
void f(E e) {
  switch (e) {
    case A y: y; break;
    case E _: break;
  }
}
";
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "pattern_never_matches_value_type"),
        vec![at(fixture, "A y", 0)],
        "the root pattern's verdict is unchanged: {:?}",
        report.diagnostics
    );
}

/// A constant leaf under a logical operand keeps its parse-time verdict: the
/// constant family already judged logical operands against the scrutinee, and
/// the shared predicate must reproduce that (`1` can never equal an `E`
/// value).
#[test]
fn constant_pattern_under_a_logical_operand_is_unchanged() {
    let fixture = "\
enum E { a }
void f(E e) {
  switch (e) {
    case E.a || 1: break;
    case E _: break;
  }
}
";
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "constant_pattern_never_matches_value_type"),
        vec![at(fixture, "1:", 0)],
        "the `||` right operand's constant is judged against `E`: {:?}",
        report.diagnostics
    );
}

/// A container field sub-pattern reads NO scrutinee: its matched type is the
/// property column (`analyzeObjectPattern`'s per-field matched type), so a
/// field whose column is `Object` can hold the required `int` even though the
/// pattern's own scrutinee (`E`) cannot. Only the object pattern's head
/// reports — the nested node stays silent, the under-report the column supply
/// (#3235's open residual) will lift.
///
/// This is the false-positive guard for the gate replacement: feeding the
/// scrutinee to the nested node would report `int` here.
#[test]
fn object_field_sub_pattern_is_not_judged_against_the_scrutinee() {
    let fixture = "\
enum E { a }
class Box {
  final Object value;
  const Box(this.value);
}
void f(E e) {
  if (e case Box(value: int x)) {
    x;
  }
}
";
    let report = report_ops(fixture);
    assert_eq!(
        offsets(&report, "pattern_never_matches_value_type"),
        vec![at(fixture, "Box(value", 0)],
        "only the object head `Box` reports; the field column stays unknown: {:?}",
        report.diagnostics
    );
}

/// The List kind gets its call site (`list_pattern_resolver.dart:45`): the
/// pattern's required type is `List<elementType>`, whose element is the
/// written type, else the matched list's element, else `Object?` — a matched
/// value that cannot be a `List` at all proves the pattern dead. The row
/// spans the whole `[...]` node (the official `.at(pattern)`).
///
/// Before #3235 the kind had no listener hook, so the switch stayed silent.
#[test]
fn list_pattern_runs_the_never_match_verdict() {
    let fixture = "\
enum E { a }
void f(E e) {
  switch (e) {
    case [var x]: x; break;
    case E _: break;
  }
}
";
    let report = report_ops(fixture);
    let needle = "[var x]";
    let rows = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "pattern_never_matches_value_type")
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect::<Vec<_>>();
    assert_eq!(
        rows,
        vec![(
            at(fixture, needle, 0),
            u32::try_from(needle.len()).expect("needle fits u32"),
            "The matched value type 'E' can never match the required type 'List<Object?>'."
                .to_string(),
        )],
        "the list pattern is judged against the scrutinee `E`: {:?}",
        report.diagnostics
    );
}

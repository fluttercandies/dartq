//! #3289 pins — the `unused_element` operator-receiver credit.
//!
//! The gather visitor records a member use for every operator access over
//! the extended type (`visitBinaryExpression`/`visitPrefixExpression`/
//! `visitPostfixExpression` → `usedElements.addMember`,
//! unused_local_elements_verifier.dart:59-64/:196-201/:256-267), so an
//! `operator` declared by an applicable extension is used even when the
//! receiver never resolves to a type in the element tables. The resolve
//! layer leaves an identifier or literal operand untyped, which left the
//! `unused_element` arm reporting a used operator: the `co19`
//! Constructor-tear-offs corpus (`extension on Function { operator< }` used
//! as `a < b`; `extension on Type { operator> }` used as `c > d`) and the
//! `"hello" - 3` string-receiver shape were the dominant failing population
//! (398 failing cases, 2026-11-01 census). `finalize_usage` now derives the
//! receiver head from the source bytes and the declaration tables — a
//! string literal is a `String`, a function-valued identifier a `Function`,
//! a type name used as a value a `Type` — and folds it through the existing
//! `credit_face_member` guard chain.
//!
//! Registered through `diagnose/tests.rs`'s `#[path]` block (`diagnose.rs`
//! is a live-lane file out of this ticket's write scope; `#[path]` keeps the
//! module inside the `diagnose` subtree so `crate::diagnose::test_rig`
//! resolves).

use crate::diagnose::test_rig::{CORE_PRELUDE, report_ops, rows};
use crate::diagnose::types::FileReport;

/// The offsets of the `unused_element` rows, ascending.
fn unused_element_offsets(report: &FileReport) -> Vec<u32> {
    let mut at = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unused_element")
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

// —— the dominant constructor-tear-off shape ——————————————————————————————

/// `co19` `Constructor-tear-offs/ambiguities_*`: a function receiver takes
/// an `on Function` operator and a type-literal receiver an `on Type`
/// operator. Both extensions declare exactly one operator spelling, so the
/// exact-one-declarer guard admits the credit and the operators stay used.
#[test]
fn function_and_type_receiver_operators_are_credited() {
    let fixture = "\
extension on Function {
  bool operator <(Type t) => true;
}
extension on Type {
  int operator >(int i) => i;
}
typedef b = int;
String a<T>(int x) => '';
void main() {
  a < b;
  b > 3;
}
";
    let report = report_ops(fixture);
    assert!(
        unused_element_offsets(&report).is_empty(),
        "the relational operators are used through their faces: {}",
        rows(&report).join("\n")
    );
}

/// The compound-assignment token maps to the base operator: `a *= 2` over a
/// function reference credits `operator*` (the `*=` spelling never names a
/// declaration).
#[test]
fn compound_assignment_operator_maps_to_the_base_operator() {
    let fixture = "\
extension on Function {
  int operator *(int other) => other;
}
int a(int x) => x;
void f() {
  a *= 2;
}
";
    let report = report_ops(fixture);
    assert!(
        unused_element_offsets(&report).is_empty(),
        "`*=` credits `operator*`: {}",
        rows(&report).join("\n")
    );
}

// —— the string-literal receiver ——————————————————————————————————————————

/// `UnusedElementTest.test_method_isUsed_privateExtension_binaryOperator`:
/// `"hello" - 3` applies an `on String` operator. A string literal is
/// unambiguously a `String` receiver, so the credit folds to the face.
#[test]
fn string_literal_receiver_operator_is_credited() {
    let fixture = "\
extension on String {
  int operator -(int other) => other;
}
void main() {
  \"hello\" - 3;
}
";
    let report = report_ops(fixture);
    assert!(
        unused_element_offsets(&report).is_empty(),
        "the String receiver credits `operator-`: {}",
        rows(&report).join("\n")
    );
}

// —— silent negatives (guards that keep the credit over-report-proof) ——————

/// A typed identifier receiver is NOT a type literal: `x > 2` over an `int`
/// local uses the native `int.>` (the prelude declares it), never the `on
/// Type` face, so the face's operator stays unused and reports.
#[test]
fn typed_identifier_receiver_does_not_steal_a_type_face_operator() {
    let fixture = "\
extension on Type {
  int operator >(int i) => i;
}
void main() {
  int x = 1;
  x > 2;
}
";
    let report = report_ops(fixture);
    let expected = at(fixture, "operator >", 0) + "operator ".len() as u32;
    assert_eq!(
        unused_element_offsets(&report),
        vec![expected],
        "the int receiver uses int.>, so the Type face's operator is unused: {}",
        rows(&report).join("\n")
    );
}

/// An operator whose receiver type no face matches stays unused: `3 - 3`
/// over `int` operands never applies the `on String` face.
#[test]
fn unmatched_receiver_operator_still_reports() {
    let fixture = "\
extension on String {
  int operator -(int other) => other;
}
void main() {
  3 - 3;
}
";
    let report = report_ops(fixture);
    let expected = at(fixture, "operator -", 0) + "operator ".len() as u32;
    assert_eq!(
        unused_element_offsets(&report),
        vec![expected],
        "the int receiver never applies the String face: {}",
        rows(&report).join("\n")
    );
}

/// A comment is never a use: a `a < b` doc example must not credit an
/// otherwise-unused `on Function` operator.
#[test]
fn comment_text_never_credits_an_operator() {
    let fixture = "\
extension on Function {
  bool operator <(Type t) => true;
}
void f() {
  // a < b
}
";
    let report = report_ops(fixture);
    let expected = at(fixture, "operator <", 0) + "operator ".len() as u32;
    assert_eq!(
        unused_element_offsets(&report),
        vec![expected],
        "the comment credits nothing: {}",
        rows(&report).join("\n")
    );
}

/// A class's own operator used natively is credited by the resolved-member
/// join (`member_refs`), never by this source-scan arm — the arm must not
/// regress that path nor spuriously report the member.
#[test]
fn native_class_operator_use_stays_silent() {
    let fixture = "\
class C {
  bool operator <(C other) => true;
}
void f() {
  C() < C();
}
";
    let report = report_ops(fixture);
    assert!(
        unused_element_offsets(&report).is_empty(),
        "the native operator is used through the resolved member join: {}",
        rows(&report).join("\n")
    );
}

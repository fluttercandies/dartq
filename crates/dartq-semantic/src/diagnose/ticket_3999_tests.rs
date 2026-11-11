//! #3999 — the layer-level guard for the dual-clause member face: a member
//! declared under BOTH an `extends` clause-arg and an `implements`
//! clause-arg types every send/read through the MERGED interface column
//! (the oracle's `getMember` over `inheritance_manager3`'s `_topMerge`),
//! never the single-edge `extends` face.
//!
//! The defect sat one layer below the send-close chain: `slot_subtype`'s
//! top-source arm judged `dynamic <: FutureOr<Object?>` false because the
//! `FutureOr` union is a composite top its literal-spelling test could not
//! see — so the `dynamic Function()` candidate dropped out of
//! `_getValidOverrides`, the merge kept candidate zero, and the stamped
//! send face read the `extends` edge's `FutureOr<Object?> Function()`.
//! Upstream's Left Top leg answers `dynamic <: T1` by recursing
//! `Object? <: T1` (subtype.dart:63-68); the fix recurses for the
//! `FutureOr` composite target only, and the `Null` arm rides the same
//! union leg (`Null <: FutureOr<T>` through `Null <: T`).

use super::FileReport;
use super::test_rig::report_ops;

/// The messages of one code's rows, in source order.
fn texts(report: &FileReport, code: &str) -> Vec<String> {
    let mut rows = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .collect::<Vec<_>>();
    rows.sort_by_key(|d| d.range.offset);
    rows.iter().map(|d| d.message.to_string()).collect()
}

fn all_rows(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .map(|d| format!("{}@{} {}", d.code.name, d.range.offset, d.message))
        .collect()
}

/// The layer-level guard: the `dynamic Function()` candidate must stay a
/// valid override of the `FutureOr<Object?> Function()` sibling, so the
/// merged column — `Object? Function()`, upstream `topMerge` over the
/// union's value leg — types the send. The `extends`-only control proves
/// the same send still reads the single edge's face when no second
/// clause-arg contributes a candidate.
#[test]
fn merged_column_wins_over_the_single_extends_edge() {
    let report = report_ops(
        "class A<T> {\n\
         \x20 T member() => throw 'unreachable';\n\
         }\n\
         class B<T> {\n\
         \x20 T member() => throw 'unreachable';\n\
         }\n\
         void takesNum(num x) {}\n\
         class DND1 extends A<FutureOr<Object?> Function()> implements B<dynamic Function()> {\n\
         \x20 void test() {\n\
         \x20   var self = this;\n\
         \x20   var y = self.member();\n\
         \x20   takesNum(y);\n\
         \x20 }\n\
         }\n",
    );
    let atna = texts(&report, "argument_type_not_assignable");
    assert_eq!(atna.len(), 1, "expected the single send face, got {atna:?}");
    assert!(
        atna[0].contains("'Object? Function()'"),
        "the send must read the merged `Object? Function()` column, got {atna:?} ({:?})",
        all_rows(&report)
    );
    assert!(
        !atna[0].contains("FutureOr"),
        "the single-edge `extends` face must not survive the merge, got {atna:?}"
    );
}

/// The single-candidate control: `extends A<FutureOr<Object?> Function()>`
/// alone keeps its written face — the merged-column read must not fabricate
/// a second face where only one clause-arg exists.
#[test]
fn single_extends_edge_keeps_its_written_face() {
    let report = report_ops(
        "class A<T> {\n\
         \x20 T member() => throw 'unreachable';\n\
         }\n\
         void takesNum(num x) {}\n\
         class DND1 extends A<FutureOr<Object?> Function()> {\n\
         \x20 void test() {\n\
         \x20   var self = this;\n\
         \x20   var y = self.member();\n\
         \x20   takesNum(y);\n\
         \x20 }\n\
         }\n",
    );
    let atna = texts(&report, "argument_type_not_assignable");
    assert!(
        atna.iter()
            .any(|m| m.contains("'FutureOr<Object?> Function()'")),
        "the single edge's `FutureOr<Object?> Function()` face must stand, \
         got {atna:?} ({:?})",
        all_rows(&report)
    );
}

//! #41983 pins — a cascade section's receiver is the cascade TARGET.
//!
//! `regress41983_test.dart` builds a list with 10,000 `..add(n)` sections
//! whose `add` returns `void`. Upstream analyzes one target per cascade and
//! every section resolves against it (`resolver.dart`'s
//! `visitCascadeExpression` gives a section's access a null target; the
//! section's AST target is the cascade's target expression), so the oracle
//! answers no `use_of_void_result` at all for that file. Reading the section
//! receiver as "the closest row before the `..`" instead picks the PREVIOUS
//! section's merged row, whose type is `void` for every `add` — one row per
//! section, 10,000 identical rows at the cascade target's widening, capped
//! only by the per-file diagnostic fuse.
//!
//! Every `dart analyze` 3.13.7 machine row quoted below is the oracle's
//! answer over a single-file package of the same fixture.

use super::FileReport;
use super::test_rig::{CORE_PRELUDE, report_ops};

/// The `use_of_void_result` rows as `(offset, length)`, fixture-relative.
fn void_rows(report: &FileReport) -> Vec<(usize, u32)> {
    report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == "use_of_void_result")
        .map(|row| {
            (
                row.range.offset as usize - CORE_PRELUDE.len(),
                row.range.length,
            )
        })
        .collect()
}

/// The #41983 shape: a cascade whose sections call a `void`-returning member
/// of the target. No section's value is used, so the oracle answers nothing
/// (probe: the 10,000-section `regress41983_test.dart` yields zero rows).
#[test]
fn cascade_void_sections_report_nothing() {
    let fixture = "void f() {\n  List<int> ints = <int>[]\n    ..add(0)\n    ..add(1)\n    ..add(2);\n}\n";
    assert_eq!(void_rows(&report_ops(fixture)), Vec::<(usize, u32)>::new());
}

/// The same cascade at corpus scale: the row count must not track the
/// section count (the defect emitted one per section, 10,000 rows in the
/// materialized case).
#[test]
fn cascade_two_hundred_void_sections_report_nothing() {
    let mut fixture = String::from("void f() {\n  List<int> ints = <int>[]\n");
    for i in 0..200 {
        fixture.push_str(&format!("    ..add({i})\n"));
    }
    fixture.push_str("  ;\n}\n");
    assert_eq!(void_rows(&report_ops(&fixture)), Vec::<(usize, u32)>::new());
}

/// A `void`-returning section on a non-`void` target reports nothing either:
/// the section value is discarded, not consumed (oracle: `A()..bar()..baz;`
/// answers no rows).
#[test]
fn cascade_void_sections_on_class_target_report_nothing() {
    let fixture = "class A {\n  void bar() {}\n  int baz = 0;\n}\nvoid main() {\n  A()..bar()..baz;\n}\n";
    assert_eq!(void_rows(&report_ops(fixture)), Vec::<(usize, u32)>::new());
}

/// The miss direction: a genuinely `void` cascade target still reports, and
/// it reports at the TARGET node — once for the whole cascade, however many
/// send sections follow (oracle `v()..bar()..bar();` → `USE_OF_VOID_RESULT`
/// at the `v()` of column 3, length 3).
#[test]
fn cascade_void_target_send_reports_once_at_target() {
    let fixture = "void v() {}\nclass A {\n  void bar() {}\n}\nvoid main() {\n  v()..bar()..bar();\n}\n";
    let report = report_ops(fixture);
    let target = fixture.find("v()..").expect("fixture target");
    assert_eq!(void_rows(&report), vec![(target, 3)]);
}

/// A read section over a `void` target anchors at its own member name, the
/// `_resolve` `targetType is VoidType` arm (oracle `v()..baz;` → column 8,
/// length 3).
#[test]
fn cascade_void_target_read_reports_at_member() {
    let fixture = "void v() {}\nclass A {\n  int baz = 0;\n}\nvoid main() {\n  v()..baz;\n}\n";
    let report = report_ops(fixture);
    let member = fixture.find("baz;").expect("fixture member");
    assert_eq!(void_rows(&report), vec![(member, 3)]);
}

/// Both section shapes in one cascade keep their own anchors (oracle
/// `v()..bar()..baz;` → rows at the target `v()` and at `baz`).
#[test]
fn cascade_void_target_send_and_read_report_their_own_rows() {
    let fixture = "void v() {}\nclass A {\n  void bar() {}\n  int baz = 0;\n}\nvoid main() {\n  v()..bar()..baz;\n}\n";
    let report = report_ops(fixture);
    let target = fixture.find("v()..").expect("fixture target");
    let member = fixture.find("baz;").expect("fixture member");
    assert_eq!(void_rows(&report), vec![(target, 3), (member, 3)]);
}

/// A chained send inside a section is a `handle_dot_access` receiver, not a
/// cascade section: `v()..bar().n()`. The section's own access target stays
/// the cascade target, so the single row keeps the target anchor (oracle
/// answers one row at `v()`).
#[test]
fn cascade_void_target_with_chained_send_reports_once() {
    let fixture = "void v() {}\nclass A {\n  A bar() => this;\n  void n() {}\n}\nvoid main() {\n  v()..bar().n();\n}\n";
    let report = report_ops(fixture);
    let target = fixture.find("v()..").expect("fixture target");
    assert_eq!(void_rows(&report), vec![(target, 3)]);
}

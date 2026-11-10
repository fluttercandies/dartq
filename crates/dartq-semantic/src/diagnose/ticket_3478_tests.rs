//! The #3478 pins — element/field credit by BARE NAME instead of by
//! declaration (~300 conformance rows, firing both directions).
//!
//! `semdiag.rs`'s library close re-decides every staged `unused_element` /
//! `unused_field` row against `consumed.used(name)` / `consumed.read(name)`
//! — a whole-library second pass at NAME granularity. Two same-spelled
//! declarations collide on the one spelling: `A._m1` and `B._m1` share the
//! name, so a use of `B._m1` releases `A._m1`'s row (the pin below), a
//! parameter shadowing a field, a duplicate declaration the oracle's
//! first-wins scope settles, a for-in scope or `_`-named local, all land
//! the credit on the wrong declaration.
//!
//! The per-file pass already resolved the element each use bound
//! (`member_refs` edges carry `(target_file, target_container,
//! target_name)`; `extension_member_used` carries `(file, face_index,
//! spelling)`). The close now re-decides against those declaration keys:
//! a member row's anchor maps to its `(container, spelling)` through the
//! declaring file's element model, and the close's `consumed.members`
//! joins the merged `member_refs` — never the bare name.
//!
//! Discriminating pin: `test_method_notUsed_hasSameNameAsUsed` must report
//! `A._m1` at 3:8. `report_ops` (the single-unit path) already reports it
//! correctly — the per-file verdict — so the pin lands on the library
//! close's re-decision; the same fixture routed through the CLI/library
//! path proves the bare-name pass was the cause when the two disagree.
//!
//! Registered here via `#[path]`; the module stays inside the `diagnose`
//! subtree so `crate::diagnose::test_rig` resolves.

use super::FileReport;
use super::test_rig::{report_ops, rows};

/// The `unused_element`/`unused_field` rows of `report`, rendered for the
/// failure message.
fn unused(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unused_element" || d.code.name == "unused_field")
        .map(|d| format!("{}@{}: {}", d.code.name, d.range.offset, d.message))
        .collect()
}

/// `test_method_notUsed_hasSameNameAsUsed`: a use of `B._m1` must not
/// spare `A._m1` — the member-level join keys on the declaring container.
/// The single-unit `report_ops` face already resolves `B._m1` through its
/// `member_refs` edge, so `A._m1` reports here; the library-close pin rides
/// the same element key (the bare name would release it).
#[test]
fn same_named_method_reports_the_unused_declaring_class() {
    let fixture = "\
class A {
  void _m1() {}
}
class B {
  void public() => _m1();
  void _m1() {}
}
";
    let report = report_ops(fixture);
    let bad = unused(&report);
    assert!(
        bad.iter().any(|r| r.contains("'_m1'")),
        "`A._m1` stays unused while `B._m1` reads: {bad:?}\n{}",
        rows(&report).join("\n")
    );
    // Exactly one `_m1` row: the used `B._m1` must not double-report.
    assert_eq!(
        bad.iter().filter(|r| r.contains("'_m1'")).count(),
        1,
        "one `_m1` verdict only: {bad:?}\n{}",
        rows(&report).join("\n")
    );
}

/// A parameter shadowing a same-spelled field: `m(int _f)` shadows `this._f`
/// in the method body, so the field's name-level use is a local's — the
/// field stays unused.
#[test]
fn parameter_shadowing_field_does_not_spare_the_field() {
    let fixture = "\
class A {
  int _f = 0;
  void m(int _f) {
    _f;
  }
}
";
    let report = report_ops(fixture);
    let bad = unused(&report);
    assert!(
        bad.iter()
            .any(|r| r.contains("unused_field") && r.contains("'_f'")),
        "the parameter's read is not the field's: {bad:?}\n{}",
        rows(&report).join("\n")
    );
}

/// A `for-in` scope local sharing the field's name: the loop variable's
/// read is not the field's.
#[test]
fn for_in_scope_does_not_spare_the_field() {
    let fixture = "\
class A {
  int _f = 0;
  void m(List<int> _f) {
    for (var x in _f) { x; }
  }
}
";
    let report = report_ops(fixture);
    let bad = unused(&report);
    assert!(
        bad.iter()
            .any(|r| r.contains("unused_field") && r.contains("'_f'")),
        "the formal's reads are not the field's: {bad:?}\n{}",
        rows(&report).join("\n")
    );
}

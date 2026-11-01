//! The #3479 pins — writes counted as reads (~75 conformance rows).
//!
//! `unused_field`'s rule is a READ rule (`_isReadIdentifier` /
//! `readMembers`, upstream Dart SDK source,
//! pkg/analyzer/lib/src/error/unused_local_elements_verifier.dart): a write
//! to a field never credits its implicit getter. The engine's read pool
//! carries three write shapes the oracle never counts: a statement-position
//! increment or compound whose target the expression statement discarded
//! (`_f++;`, `_f += 1;` — the statement-window discard already lands in
//! `statement_discarded_rows` for the pass-2 read gate, but the
//! finalize-side element verdict re-joined the spelling), a constructor
//! initializer's LHS write (`A() : _f = 0`), and an initializing formal's
//! write-side use (`this._x`). A MEMBER-target compound is different: in
//! `a._f += 1;` the identifier's parent is the property access, not the
//! assignment, so the oracle counts the getter read — the field stays
//! used (the same answer `ticket_2409`'s `member_compound_target_spares`
//! and `this_member_compound_target_spares` pin).
//!
//! The pin the brief names: `class A { int _f = 0; void m() { _f += 2; } }`
//! reports `unused_field`; the expression form `int m() => _f += 2;` does
//! not (the body's read reaches the getter).
//!
//! Registered here via `#[path]`; the module stays inside the `diagnose`
//! subtree so `crate::diagnose::test_rig` resolves.

use super::test_rig::{report_ops, rows};
use super::FileReport;

/// The `unused_field`/`unused_element` rows of `report` (the family's
/// lens — the fixtures pin the exact verdict).
fn unused(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unused_element" || d.code.name == "unused_field")
        .map(|d| format!("{}@{}: {}", d.code.name, d.range.offset, d.message))
        .collect()
}

/// The statement-position compound assignment is a pure write — the field
/// reports `unused_field` (`test_notUsed_compoundAssign`).
#[test]
fn statement_compound_reports_unused_field() {
    let fixture = "\
class A {
  int _f = 0;
  void m() {
    _f += 2;
  }
}
";
    let report = report_ops(fixture);
    let bad = unused(&report);
    assert!(
        bad.iter().any(|r| r.contains("unused_field") && r.contains("'_f'")),
        "the discarded `_f += 2` never reads the getter: {bad:?}\n{}",
        rows(&report).join("\n")
    );
}

/// The statement-position increment reports the same (`test_notUsed_
/// postfixExpr` / `test_notUsed_prefixExpr`).
#[test]
fn statement_postfix_reports_unused_field() {
    let fixture = "\
class A {
  int _f = 0;
  void m() {
    _f++;
  }
}
";
    let report = report_ops(fixture);
    let bad = unused(&report);
    assert!(
        bad.iter().any(|r| r.contains("unused_field") && r.contains("'_f'")),
        "the discarded `_f++` never reads the getter: {bad:?}\n{}",
        rows(&report).join("\n")
    );
}

/// A MEMBER-target statement compound READS the getter — in `a._f += 1;`
/// the identifier's parent is the property access, not the assignment,
/// so the oracle's `_isReadIdentifier` counts the read and the field
/// stays used. Oracle answer: spare — the same verdict `ticket_2409`'s
/// `member_compound_target_spares` / `this_member_compound_target_spares`
/// pin for the identical member-target compound shape. (The earlier
/// "reports" form of this pin passed only because `member_read_discarded`
/// over-stripped every compound suffix.)
#[test]
fn member_statement_compound_spares_the_field() {
    let fixture = "\
class A {
  int _f = 0;
}
void f(A a) {
  a._f += 1;
}
";
    let report = report_ops(fixture);
    let bad = unused(&report);
    assert!(
        !bad.iter().any(|r| r.contains("'_f'")),
        "the `a._f += 1` compound reads the getter — the field stays used: {bad:?}\n{}",
        rows(&report).join("\n")
    );
}

/// The LOCAL half of the ticket's thesis — a write is not a read: `y = 1`
/// assigns a local the statement discards, so `y` is never read after its
/// initializer and `unused_local_variable` reports.
#[test]
fn local_statement_write_reports_unused_local() {
    let fixture = "\
void f() {
  var y = 0;
  y = 1;
}
";
    let report = report_ops(fixture);
    let bad = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unused_local_variable")
        .map(|d| format!("{}@{}: {}", d.code.name, d.range.offset, d.message))
        .collect::<Vec<_>>();
    assert!(
        bad.iter().any(|r| r.contains("'y'")),
        "the discarded `y = 1` never reads the local: {bad:?}\n{}",
        rows(&report).join("\n")
    );
}

/// A constructor-initializer LHS write is a pure write — `A() : _f = 0`
/// never reads the getter (`test_notUsed_constructorFieldInitializers`).
#[test]
fn constructor_initializer_write_reports_unused_field() {
    let fixture = "\
class A {
  int _f;
  A() : _f = 0;
}
";
    let report = report_ops(fixture);
    let bad = unused(&report);
    assert!(
        bad.iter().any(|r| r.contains("unused_field") && r.contains("'_f'")),
        "the `_f = 0` initializer never reads the getter: {bad:?}\n{}",
        rows(&report).join("\n")
    );
}

/// The negative twin the brief names: an EXPRESSION-position compound
/// (`int m() => _f += 2;`) is a real read — the field stays silent.
#[test]
fn expression_compound_reads_the_field() {
    let fixture = "\
class A {
  int _f = 0;
  int m() => _f += 2;
}
";
    let report = report_ops(fixture);
    let bad = unused(&report);
    assert!(
        !bad.iter().any(|r| r.contains("'_f'")),
        "the `_f += 2` expression reads the getter: {bad:?}\n{}",
        rows(&report).join("\n")
    );
}

//! The #2765 pins: the chained promoted-property receiver —
//! `c._p.isEven` under `if (c._p != null)` resolves `.isEven` on the
//! promoted non-null face of the property read (the official read's
//! static type IS the promoted one,
//! property_element_resolver.dart:2158-2166), so the guarded read stays
//! silent while the read after a receiver reassignment still reports.
//! The flow layer stages the promotion fact at the receiver read's member
//! token; the pass-2 receiver verdict joins it through the #2283 chained
//! latch's zero-length marker, whose offset is that token — an exact-span
//! staging compare dropped the join and every chained receiver
//! over-reported. The direct member read (a staged check's own span) and
//! the bare implicit-`this` read (the head latch's real span) were never
//! affected.

use super::test_rig::{CORE_PRELUDE, report_ops};
use super::*;

fn unchecked(rep: &FileReport) -> Vec<&Diagnostic> {
    rep.diagnostics
        .iter()
        .filter(|d| d.code.name == "unchecked_use_of_nullable_value")
        .collect()
}

fn count_code(rep: &FileReport, name: &str) -> usize {
    rep.diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .count()
}

/// The rows of a report over `full` rendered as
/// `SEVERITY|code|line:col|len|message` (source order). Line/column are
/// the 1-based UTF-16 positions the oracle's rows carry; the file column
/// is the rig's own single file (the corpus spells it `lib/test.dart`).
fn rows(full: &str, rep: &FileReport) -> Vec<String> {
    let mut line_starts = vec![0u32];
    for (index, byte) in full.bytes().enumerate() {
        if byte == b'\n' {
            line_starts.push(index as u32 + 1);
        }
    }
    let line_col = |offset: u32| {
        let line = line_starts
            .binary_search_by(|start| start.cmp(&offset))
            .unwrap_or_else(|at| at - 1);
        (line + 1, offset - line_starts[line] + 1)
    };
    rep.diagnostics
        .iter()
        .map(|d| {
            let (line, col) = line_col(d.range.offset);
            let severity = match d.severity {
                Severity::Error => "ERROR",
                Severity::Warning => "WARNING",
                Severity::Info => "INFO",
            };
            format!(
                "{severity}|{}|{line}:{col}|{}|{}",
                d.code.name,
                d.range.length,
                d.message
            )
        })
        .collect()
}

/// The conformance corpus case
/// `manual/diagnostic/unchecked_use_of_nullable_value/7`'s trigger shape:
/// the guarded read is clean (the promotion stands), the read after the
/// receiver reassignment reports at `isEven`'s own token. The rig's
/// dart:core projection carries no `isEven`, so the fixture adds it as an
/// extension getter — the row's anchor, length, and message stay the
/// oracle's. Expected.tsv carries this single row (`lib/test.dart|12:8`
/// with `class C {` at line 1; the rig's one-line prelude plus the
/// extension line put the same read at 14:8 here).
#[test]
fn case_rows_match_the_oracle_guarded_read_silent_reassigned_read_reports() {
    let fixture = "\
extension I on int { bool get isEven => true; }
class C {
  final int? _p;

  C(this._p);
}

void f(C c) {
  if (c._p != null) {
    c._p.isEven;
  }
  c = C(1);
  c._p.isEven;
}
";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let rep = report_ops(fixture);
    assert_eq!(
        rows(&full, &rep),
        ["ERROR|unchecked_use_of_nullable_value|14:8|6|The property 'isEven' can't be unconditionally accessed because the receiver can be 'null'."],
        "{:?}",
        rep.diagnostics
    );
}

/// The same guard without the reassignment — the divergence site's own
/// shape (the flow audit's variant A): the chained read is silent.
#[test]
fn guarded_chained_read_stays_silent_without_the_reassignment() {
    let fixture = "\
extension I on int { bool get isEven => true; }
class C {
  final int? _p;

  C(this._p);
}

void f(C c) {
  if (c._p != null) {
    c._p.isEven;
  }
}
";
    let rep = report_ops(fixture);
    assert!(
        rows(&format!("{CORE_PRELUDE}{fixture}"), &rep).is_empty(),
        "{:?}",
        rep.diagnostics
    );
}

/// A local receiver reads the same rescue (the audit's variant C): the
/// chained latch rides the bare head's latch regardless of how the
/// receiver local was bound.
#[test]
fn guarded_chained_read_stays_silent_for_a_local_receiver() {
    let fixture = "\
extension I on int { bool get isEven => true; }
class C {
  final int? _p;

  C(this._p);
}

void f() {
  var c = C(1);
  if (c._p != null) {
    c._p.isEven;
  }
}
";
    let rep = report_ops(fixture);
    assert!(
        rows(&format!("{CORE_PRELUDE}{fixture}"), &rep).is_empty(),
        "{:?}",
        rep.diagnostics
    );
}

/// The direct read in the guard was never broken (its staged check joins
/// at its own span) and keeps its silence: the promoted `int` assigns to
/// `int`, no `invalid_assignment` stands.
#[test]
fn guarded_direct_read_keeps_its_assignment_silence() {
    let fixture = "\
class C {
  final int? _p;

  C(this._p);
}

void f(C c) {
  if (c._p != null) {
    int y = c._p;
  }
}
";
    let rep = report_ops(fixture);
    assert_eq!(
        count_code(&rep, "invalid_assignment"),
        0,
        "{:?}",
        rep.diagnostics
    );
}

/// The join stays promotion-gated: an unguarded chained read over the
/// same promotable field still reports — the relaxation is the staging
/// join's key, never a general silencing.
#[test]
fn unguarded_chained_read_still_reports() {
    let fixture = "\
extension I on int { bool get isEven => true; }
class C {
  final int? _p;

  C(this._p);
}

void f(C c) {
  c._p.isEven;
}
";
    let rep = report_ops(fixture);
    assert_eq!(unchecked(&rep).len(), 1, "{:?}", rep.diagnostics);
}

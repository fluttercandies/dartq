//! The #2769 pins: the keyword-receiver head latch — `this._p.isEven`
//! under `if (this._p != null)` resolves `.isEven` on the promoted
//! non-null face of the property read exactly like the implicit-`this`
//! bare spelling: the official promotion lookup keys the property slot
//! on the same `this` identity either way (`_handleProperty`'s target
//! SSA node — `ThisPropertyTarget._getSsaNode` and an
//! `ExpressionPropertyTarget` naming `this` answer the same node,
//! upstream Dart SDK source,
//! pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart),
//! so the #1465-style head latch must arm for the explicit keyword row
//! too and the #2283 chained latch links through it. The promotion
//! stays flow-scoped: a read after the guard's block still reports,
//! and an unguarded read never loses its row.

use super::test_rig::{CORE_PRELUDE, report_ops};
use super::*;

fn count_code(rep: &FileReport, name: &str) -> usize {
    rep.diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .count()
}

/// The rows of a report over `full` rendered as
/// `SEVERITY|code|line:col|len|message` (source order), mirroring the
/// ticket_2765 rig's renderer (1-based UTF-16 line:col).
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

/// The guarded keyword-receiver chain is silent (the oracle's behavior):
/// the promotion fact stages at `_p`'s token for the explicit `this`
/// spelling and the chained link joins it.
#[test]
fn guarded_this_chained_read_stays_silent() {
    let fixture = "\
extension I on int { bool get isEven => true; }
class C {
  final int? _p;

  C(this._p);

  void f() {
    if (this._p != null) {
      this._p.isEven;
    }
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

/// The promotion gate holds for the keyword spelling: an unguarded
/// `this._p.isEven` still reports at `isEven`'s own token — the latch
/// link keys the staged promotion's join, never a general silencing.
#[test]
fn unguarded_this_chained_read_still_reports() {
    let fixture = "\
extension I on int { bool get isEven => true; }
class C {
  final int? _p;

  C(this._p);

  void f() {
    this._p.isEven;
  }
}
";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let rep = report_ops(fixture);
    assert_eq!(
        rows(&full, &rep),
        ["ERROR|unchecked_use_of_nullable_value|9:13|6|The property 'isEven' can't be unconditionally accessed because the receiver can be 'null'."],
        "{:?}",
        rep.diagnostics
    );
}

/// The promotion is flow-scoped for the keyword spelling too: the guard
/// block closed, so the later read keeps its row.
#[test]
fn read_after_the_guard_block_still_reports() {
    let fixture = "\
extension I on int { bool get isEven => true; }
class C {
  final int? _p;

  C(this._p);

  void f() {
    if (this._p != null) {
    }
    this._p.isEven;
  }
}
";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let rep = report_ops(fixture);
    assert_eq!(
        rows(&full, &rep),
        ["ERROR|unchecked_use_of_nullable_value|11:13|6|The property 'isEven' can't be unconditionally accessed because the receiver can be 'null'."],
        "{:?}",
        rep.diagnostics
    );
}

/// The direct keyword-receiver read in the guard keeps its silence: the
/// promoted `int` assigns to `int`, no `invalid_assignment` stands.
#[test]
fn guarded_this_direct_read_keeps_its_assignment_silence() {
    let fixture = "\
class C {
  final int? _p;

  C(this._p);

  void f() {
    if (this._p != null) {
      int y = this._p;
    }
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

/// The unguarded direct read still reports — the new link rides the
/// staged promotion, whose absence leaves every verdict as-is.
#[test]
fn unguarded_this_direct_read_still_reports() {
    let fixture = "\
class C {
  final int? _p;

  C(this._p);

  void f() {
    int y = this._p;
  }
}
";
    let rep = report_ops(fixture);
    assert_eq!(
        count_code(&rep, "invalid_assignment"),
        1,
        "{:?}",
        rep.diagnostics
    );
}

//! #3991 pins — a plain `=` write target never reads the target's READ
//! face (upstream `resolveForWrite` sets `hasRead` false,
//! assignment_expression_resolver.dart:42-48, so the getter lookup and its
//! `isBottom` exit never reach flow analysis).
//!
//! The bug: a bare `Expression` identifier resolving to a top-level
//! `Never` getter fed the recorder's resolved-Top arm, whose `is_bottom`
//! exit opened a dead window from the write's own right operand. The fix
//! records the write target under the member-selector context — the same
//! flow shape the dotted `c.x = e` write already rides — so the replay
//! pushes an opaque-typed slot instead of resolving the read face.
//!
//! Oracle provenance (`~/.dartq-oracle/3.13.7/dart-sdk/bin/dart analyze
//! --format=machine`): `unreachable = 3` reports only the setter-missing
//! error on the target (`ASSIGNMENT_TO_FINAL`, 3:3) — no `DEAD_CODE` after
//! it; `unreachable;` reads dead-code the following statement; `??=` both
//! errors the target and dead-codes the RHS region; `+=` errors the target
//! with NO dead row (a residual — see below).
//!
//! Known residuals outside this lane's scope:
//! - dartq reports no `ASSIGNMENT_TO_FINAL` row for a bare top-level getter
//!   write at all (the `resolve_bare_row` `RefTarget::Decl` arm carries no
//!   write-intent face) — owner: row_resolution.rs's bare Decl arm.
//! - `unreachable += 3` still dead-codes: upstream reads the getter under
//!   `hasRead` but a STATIC getter's `getType` face stays null so no exit
//!   fires — owner: the flow `Resolved::Top` arm's unconditional
//!   `is_bottom` exit on compound-operator targets.

use super::test_rig::{code_names, report_ops, report_ops_bare, rows};
use crate::diagnose::FileReport;
use dartq_diagnostics::Diagnostic;

fn dead_code_rows(report: &FileReport) -> Vec<&Diagnostic> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dead_code")
        .collect()
}

/// The item: a plain `=` write to a top-level `Never` getter opens no dead
/// window — the write resolves through the `name=` setter spelling (which
/// does not exist here — upstream's `ASSIGNMENT_TO_FINAL` row on the
/// target is the setter-missing face; dartq's missing row is the noted
/// row_resolution.rs residual), and `print('after')` stays live. The
/// oracle's exact face is `ASSIGNMENT_TO_FINAL` only; dartq asserts the
/// `dead_code` absence (the setter-missing error's own row gap is the
/// residual above, pinned loosely so a later fix does not collide).
#[test]
fn plain_write_to_never_getter_opens_no_dead_window() {
    let source = r#"Never get unreachable => throw 0;
void main() {
  unreachable = 3;
  print('after');
}
"#;
    let report = report_ops_bare(source);
    assert!(dead_code_rows(&report).is_empty(), "{:#?}", rows(&report));
}

/// The backward/forward twin: the getter declared AFTER `main` resolves
/// through the same setter spelling — the staged forward-reference channel
/// already skips write targets (the #3966 `is_write` gate), and the
/// recorder-context override makes the flow event agree.
#[test]
fn forward_never_getter_write_target_opens_no_dead_window() {
    let source = r#"void main() {
  unreachable = 3;
  print('after');
}

Never get unreachable => throw 0;
"#;
    let report = report_ops_bare(source);
    assert!(dead_code_rows(&report).is_empty(), "{:#?}", rows(&report));
}

/// The member twin: a bare `x = 3` inside `C.m()` where `x` is a `Never`
/// member getter resolves through `x=`'s setter — upstream reports
/// `ASSIGNMENT_TO_FINAL_NO_SETTER` on the target and nothing after. The
/// setter-missing row already fires (the member arm's write face); the
/// `dead_code` absence is the fix's face.
#[test]
fn plain_write_to_never_member_getter_opens_no_dead_window() {
    let source = r#"class C {
  Never get x => throw 0;
  void m() {
    x = 3;
    print('after');
  }
}
"#;
    let report = report_ops_bare(source);
    assert!(
        code_names(&report).contains(&"assignment_to_final_no_setter"),
        "{:#?}",
        rows(&report)
    );
    assert!(dead_code_rows(&report).is_empty(), "{:#?}", rows(&report));
}

/// The control: a genuine `unreachable;` READ of the `Never` getter still
/// dead-codes the following statement (the `isBottom` exit is correct on
/// a real read — the #3966 fixture family's own face).
#[test]
fn genuine_never_getter_read_still_dead_codes() {
    let source = r#"Never get unreachable => throw 0;
void main() {
  unreachable;
  print('after');
}
"#;
    let report = report_ops_bare(source);
    let hits = dead_code_rows(&report);
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
    assert_eq!(hits[0].message, "Dead code.", "{:#?}", rows(&report));
}

/// The `??=` twin: the null-aware write genuinely reads the target
/// upstream (`ifNullAssignment` resolves `hasRead` true — the read's
/// bottom exit and the `dead_null_aware_expression` row both fire), so the
/// target keeps its `Expression` event and the dead rows stand.
#[test]
fn if_null_write_to_never_getter_still_dead_codes() {
    let source = r#"Never get unreachable => throw 0;
void main() {
  unreachable ??= 3;
  print('after');
}
"#;
    let report = report_ops_bare(source);
    assert!(
        code_names(&report).contains(&"dead_code"),
        "{:#?}",
        rows(&report)
    );
    assert!(
        code_names(&report).contains(&"dead_null_aware_expression"),
        "{:#?}",
        rows(&report)
    );
}

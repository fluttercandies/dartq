//! #3245 pins — the `unused_local_variable` supply faces: pattern binders
//! (C9) and the catch clause's stack-trace split (C11).
//!
//! Upstream's rule walks the enclosing executable's `LocalVariableElement`s
//! (`pkg/linter/lib/src/rules/unused_local_variable.dart`) and reports the
//! ones whose read count is zero; the pattern binders a `case` declares are
//! ordinary locals of that executable, so `(int a, int b)` with only `a`
//! read reports `b`, and a `&& var r` guard binder reports `r`. A catch
//! clause's second parameter is the STACK TRACE, whose own code is
//! `unused_catch_stack` (`pkg/linter/lib/src/rules/unused_catch_stack.dart`)
//! — it must never surface as `unused_local_variable`.
//!
//! These pins lock the faces #3245 found ALREADY GREEN on HEAD (the
//! pre-#3058 census that cut C9-C11 counted rows these later lands closed);
//! they exist so the shapes cannot regress while the residuals (parser
//! recovery `<unnamed>` rows, nested guard binders) are chased separately.
//!
//! Registered through `diagnose/tests.rs`'s `#[path]` block (`diagnose.rs`
//! is a live-lane file out of this ticket's write scope).

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

/// One unused-local row for `name`, anchored at its binder token.
fn unused(name: &str, offset: u32) -> (u32, u32, String) {
    (
        offset,
        1,
        format!("The value of the local variable '{name}' isn't used."),
    )
}

/// C9: an `if-case` record pattern reports the unreferenced binders of both
/// the plain shape and the `&& var` guard composite, and only those.
#[test]
fn if_case_pattern_binders_report_unused_locals() {
    let fixture = "\
void f(Object o) {
  if (o case (int a, int b)) {
    a;
  }
  if (o case (int c, int d) && var r) {
    c;
  }
}
";
    let report = report_ops(fixture);
    let expected = vec![
        unused("b", at(fixture, "int b", 0) + 4),
        // The guard composite's own binders: `d` from the record, `r` from
        // the `&& var` operand.
        unused("d", at(fixture, "int d", 0) + 4),
        unused("r", at(fixture, "var r", 0) + 4),
    ];
    assert_eq!(rows(&report, "unused_local_variable"), expected);
}

/// C9/C11: a switch-statement case's pattern binder is the same local face,
/// and the catch clause's second parameter is `unused_catch_stack` — never
/// `unused_local_variable`.
#[test]
fn switch_case_binders_and_catch_split_keep_their_own_codes() {
    let fixture = "\
void g(Object o) {
  try {
    o;
  } catch (e, st) {
    e;
  }
  switch (o) {
    case (int m, int n):
      m;
    case _:
      break;
  }
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "unused_local_variable"),
        vec![unused("n", at(fixture, "int n", 0) + 4)],
        "the catch clause's stack trace must not land in the local-variable rule"
    );
    assert_eq!(
        rows(&report, "unused_catch_stack"),
        vec![(
            at(fixture, "st", 0),
            2,
            "The stack trace variable 'st' isn't used and can be removed.".to_string(),
        )]
    );
}

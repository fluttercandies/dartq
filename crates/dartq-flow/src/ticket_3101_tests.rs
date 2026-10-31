//! #3101 — the dead `rethrow;` statement row.
//!
//! A `rethrow;` is an `ExpressionStatement` (`RethrowExpression` inside
//! one), so it keeps that statement wrapper's contract: the wrapper's dead
//! poll anchors a dead row at the statement's first token — the `rethrow`
//! keyword (`DeadCodeVerifier._visitNode`, upstream
//! dead_code_verifier.dart:183-205) — and `StmtEnd` extends it across the
//! trailing `;`. The recorder's dedicated rethrow hooks used to drop the
//! keyword, so a dead `rethrow;` reported nothing: the corpus case
//! `test_switchCase_final_rethrow` expects `11:9|8` (`rethrow;`) and got
//! `[]`. Oracle A/B (`dart analyze` 3.13.1) on the fixtures below.

use crate::ticket_3096_tests::dead_of;

/// Oracle `9:9|8`: the `finally { return; }` kills the flow, so the
/// following `rethrow;` is the first dead node and rows its own extent.
#[test]
fn dead_rethrow_statement_rows_its_own_extent() {
    let source = "\
void f(int a) {
  try {
  } on int {
    switch (a) {
      case 0:
        try {} finally {
          return;
        }
        rethrow;
    }
  }
}
";
    assert_eq!(dead_of(source), vec!["rethrow;".to_string()]);
}

/// Oracle: no rows — a reachable `rethrow;` stays silent (the wrapper's
/// poll needs an unreachable statement).
#[test]
fn live_rethrow_statement_stays_silent() {
    let source = "\
void f() {
  try {
    throw 0;
  } catch (e) {
    rethrow;
  }
}
";
    assert_eq!(dead_of(source), Vec::<String>::new());
}

/// Oracle: no rows — the statement AFTER a live `rethrow;` is dead, and
/// `StmtEnd` keeps carrying that over (the wrapper must not steal it).
#[test]
fn statement_after_live_rethrow_is_dead() {
    let source = "\
void f() {
  try {
    throw 0;
  } catch (e) {
    rethrow;
    2;
  }
}
";
    assert_eq!(dead_of(source), vec!["2;".to_string()]);
}
